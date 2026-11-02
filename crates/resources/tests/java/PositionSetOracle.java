import java.util.*;

public class PositionSetOracle {
  record Pos(int x,int y,int z) {
    @Override public int hashCode(){return (y+z*31)*31+x;}
  }
  static Pos point(int scenario,int i){
    return switch(scenario){
      case 0 -> new Pos(i*64,0,0);
      case 1 -> new Pos((i-150)*64,0,0);
      case 2 -> new Pos(i*256,0,0);
      case 3 -> new Pos((i%2==0?i:-i)*256,0,0);
      default -> throw new AssertionError();
    };
  }
  static void emit(int scenario,int step,HashSet<Pos> set){
    var parts=new ArrayList<String>();for(var p:set)parts.add(p.x()+","+p.y()+","+p.z());
    System.out.println("C "+scenario+" "+step+" "+String.join(";",parts));
  }
  public static void main(String[] args){
    for(int scenario=0;scenario<4;scenario++){
      var set=new HashSet<Pos>();int step=0;
      for(int i=0;i<100;i++){set.add(point(scenario,i));emit(scenario,step++,set);}
      for(int i=100;i<240;i++){
        var iterator=set.iterator();iterator.next();iterator.remove();emit(scenario,step++,set);
        set.add(point(scenario,i));emit(scenario,step++,set);
        if(i%7==0){set.add(point(scenario,i));emit(scenario,step++,set);}
        if(i%9==0){var iterator2=set.iterator();iterator2.next();iterator2.remove();emit(scenario,step++,set);}
      }
      while(!set.isEmpty()){var iterator=set.iterator();iterator.next();iterator.remove();emit(scenario,step++,set);}
      for(int i=300;i<320;i++){set.add(point(scenario,i));emit(scenario,step++,set);}
    }
  }
}
